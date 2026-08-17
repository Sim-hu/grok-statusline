#!/usr/bin/env bash
# Claude Code compatible statusline command.
# grok-statusline pipes session JSON to stdin (same idea as Claude's statusLine.command).
# Do not use a heredoc here — stdin must stay as the JSON payload.
set -euo pipefail
exec python3 -c '
import json, os, sys
data = json.load(sys.stdin)
model = data.get("model", {}).get("display_name") or "grok"
cwd = data.get("workspace", {}).get("current_dir") or data.get("cwd") or ""
pct = int(data.get("context_window", {}).get("used_percentage") or 0)
filled = pct * 10 // 100
bar = "█" * filled + "░" * (10 - filled)
print(f"[{model}] {os.path.basename(cwd)} | {bar} {pct}%")
'
