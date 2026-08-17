# grok-statusline

Claude Code と同じく、Grok Build の**画面下**に常時バーを出す。

Grok には `/statusline` も描画 hook も無い。`grok-sl` は `grok` を 1〜3 行短い PTY で起動し、空けた最終行にバーを描く。

デフォルトは Claude 公式ドキュメントの multi-line 例と同じ構成:

```
[Grok 4.6]  resources  main +2 ~1  high
████░░░░░░ 24% 124k/500k │ use 4% wk 6d │ 14m │ +709/-21
```

カスタム時は Claude と同じ契約: **JSON を stdin に流し、stdout がバーになる**。

## Install

```bash
git clone https://github.com/Sim-hu/grok-statusline.git
ln -sfn "$PWD/grok-statusline/bin/grok-sl" ~/.local/bin/grok-sl
```

Python 3.10+。外部パッケージは不要。`grok` が PATH にあること。

## Usage

```bash
grok-sl                 # grok の代わりに起動
grok-sl --once          # バーだけ表示
grok-sl --dump-json     # スクリプト用 JSON を確認
grok-sl --no-usage      # billing API を叩かない
grok-sl --height 1      # 1 行に畳む
```

毎回 `grok` で出したい場合:

```bash
alias grok='grok-sl'
```

今開いているセッションの下には生えない。**次から `grok-sl` で起動したセッション**の下に出る。

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
    "command": "~/src/grok-statusline/examples/statusline.sh",
    "padding": 0
  }
}
```

`COLUMNS` / `LINES` をセットしてからコマンドを実行する。タイムアウトは 200ms。失敗したら builtin に戻す。

## JSON (Claude-compatible)

`--dump-json` で中身を見られる。Claude Code の statusline stdin に寄せている。

| Field | 内容 |
|------|------|
| `model.id` / `model.display_name` | `grok-4.6` / `Grok 4.6` |
| `workspace.current_dir` | cwd |
| `workspace.repo` | origin から host/owner/name |
| `context_window.used_percentage` | ctx 使用率 |
| `context_window.context_window_size` | ウィンドウ token |
| `context_window.total_input_tokens` | 使用 token |
| `cost.total_duration_ms` | セッション経過 |
| `cost.total_lines_added` / `removed` | エージェントの +/- |
| `rate_limits.seven_day.used_percentage` | 週次クレジット（無ければ monthly を five_hour に載せる） |
| `effort.level` | `high` など |
| `session_name` | 生成タイトル |
| `git.*` | branch, staged, modified（Grok 拡張） |

出典: `~/.grok/sessions/**/signals.json` と `summary.json`。billing は `GET /v1/billing`（60 秒キャッシュ）。

## Performance

- git は 5 秒キャッシュ。巨大 repo では untracked を数えない（`git.untracked`）
- session JSON は mtime が変わるまで再読しない
- billing はバックグラウンドスレッド
- 描画は内容が変わったときだけ。子の再描画で消えた行はすぐ書き戻す

## License

MIT
