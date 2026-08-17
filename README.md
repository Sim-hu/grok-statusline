# grok-statusline

Grok Build の画面下に、Claude Code と同じ常時バーを出す。Rust 製。

Grok には `/statusline` も描画 hook も無い。インストールは公式の `~/.grok/bin/grok` を触らない。ラッパーを `~/.local/bin/grok` に置き、シェル設定の末尾でそこを PATH の手前にする。Grok の自動更新はそのまま公式バイナリを差し替えられ、次の `grok` 起動で新しい本体を包む。**alias は不要。**

```
[Grok 4.6]  resources  main +2 ~1  high
████░░░░░░ 24% 124k/500k │ use 4% wk 6d │ 14m │ +709/-21
```

## Install

### ソースから

Rust 1.74+。

```bash
git clone https://github.com/Sim-hu/grok-statusline.git
cd grok-statusline
cargo install --path .
grok-statusline install
```

これで次から `grok` の下にバーが出る。

### Nix

```bash
# flake
nix profile install github:Sim-hu/grok-statusline
grok-statusline install

# その場だけ
nix run github:Sim-hu/grok-statusline -- install

# 開発シェル
nix develop github:Sim-hu/grok-statusline
```

`default.nix` もあるので flake なしでも `nix-build` できる。

外すとき:

```bash
grok-statusline uninstall
```

Grok 本体の自動更新でバーが消えることはない。公式バイナリは `~/.grok/bin/grok` のまま更新される。Grok のインストーラが PATH ブロックをファイル末尾に書き足した場合だけ、もう一度 `grok-statusline install`（ブロックを末尾に戻す）。

## Usage

```bash
grok                            # 普段どおり。バー付き
grok-statusline once            # バーだけ表示
grok-statusline dump-json       # スクリプト用 JSON
grok-statusline once --no-usage
grok-statusline once --height 1
```

## Config

`~/.grok/grok-statusline.json`（プロジェクトなら `<cwd>/.grok/grok-statusline.json`）。

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

Claude と同じく自前コマンドにもできる:

```json
{
  "statusLine": {
    "type": "command",
    "command": "~/src/grok-statusline/examples/statusline.sh"
  }
}
```

stdin に Claude 互換 JSON、stdout がバー。`COLUMNS` / `LINES` を渡す。200ms で失敗したら builtin。

## JSON

`grok-statusline dump-json` で確認できる。

| Field | 内容 |
|------|------|
| `model.id` / `model.display_name` | `grok-4.6` / `Grok 4.6` |
| `workspace.current_dir` | cwd |
| `workspace.repo` | origin の host/owner/name |
| `context_window.used_percentage` | ctx 使用率 |
| `cost.total_duration_ms` | セッション経過 |
| `cost.total_lines_added` / `removed` | +/- |
| `rate_limits.seven_day` | 週次クレジット |
| `effort.level` | `high` など |
| `git.*` | branch, staged, modified |

## License

MIT
