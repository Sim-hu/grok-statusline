#!/usr/bin/env python3
"""Claude-style bottom statusline for Grok Build.

Grok has no statusline hook. This wraps `grok` in a PTY shorter than the
real terminal and paints the reserved last rows itself. Child ED/CUP can
wipe those rows, so they are restored after child output.

The default renderer is a 2-line bar (model/git + context progress).
Alternatively, a user command receives Claude-shaped JSON on stdin and
its stdout becomes the bar — same contract as Claude Code's statusLine.
"""

from __future__ import annotations

import fcntl
import json
import os
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import termios
import threading
import time
import tty
import unicodedata
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from urllib.parse import quote, urlparse

VERSION = "0.2.0"
CREDITS_URL = "https://cli-chat-proxy.grok.com/v1/billing?format=credits"
MONTHLY_URL = "https://cli-chat-proxy.grok.com/v1/billing"
ANSI_RE = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\]8;;[^\x07\x1b]*(?:\x07|\x1b\\)")
DEFAULT_WINDOW = 500_000
MAX_HEIGHT = 3

BG = "\033[48;5;236m"
FG = "\033[38;5;252m"
DIM = "\033[38;5;245m"
CYAN = "\033[38;5;117m"
GREEN = "\033[38;5;114m"
YELLOW = "\033[38;5;221m"
RED = "\033[38;5;210m"
RESET = "\033[0m"


def grok_home() -> Path:
    raw = os.environ.get("GROK_HOME")
    return Path(raw).expanduser() if raw else Path.home() / ".grok"


def encode_cwd(cwd: str) -> str:
    parts = [p for p in Path(cwd).resolve().parts if p != "/"]
    return "%2F" + "%2F".join(quote(p, safe="") for p in parts)


def read_json(path: Path) -> Any | None:
    try:
        return json.loads(path.read_text(encoding="utf8"))
    except (OSError, json.JSONDecodeError):
        return None


class MtimeCache:
    """Re-read a JSON file only when mtime/size change."""

    def __init__(self) -> None:
        self._store: dict[str, tuple[int, int, Any]] = {}

    def get(self, path: Path) -> Any | None:
        try:
            st = path.stat()
        except OSError:
            return None
        key = str(path)
        hit = self._store.get(key)
        if hit and hit[0] == st.st_mtime_ns and hit[1] == st.st_size:
            return hit[2]
        data = read_json(path)
        self._store[key] = (st.st_mtime_ns, st.st_size, data)
        return data


JSON_CACHE = MtimeCache()


def vis_width(text: str) -> int:
    n = 0
    for ch in ANSI_RE.sub("", text):
        if unicodedata.combining(ch):
            continue
        n += 2 if unicodedata.east_asian_width(ch) in ("F", "W") else 1
    return n


def clip(text: str, width: int) -> str:
    if width <= 0:
        return ""
    if vis_width(text) <= width:
        return text
    out: list[str] = []
    n = 0
    for ch in text:
        w = 0 if unicodedata.combining(ch) else (2 if unicodedata.east_asian_width(ch) in ("F", "W") else 1)
        if n + w > width - 1:
            break
        out.append(ch)
        n += w
    return "".join(out) + "…"


def pad_bar(inner: str, cols: int, padding: int) -> str:
    left = max(0, padding)
    body = (" " * left) + inner
    used = vis_width(body)
    if used > cols:
        body = clip(ANSI_RE.sub("", body), cols)
        used = vis_width(body)
    return f"{BG}{FG}{body}{' ' * max(0, cols - used)}{RESET}"


def fmt_tokens(n: int) -> str:
    if n >= 1_000_000:
        return f"{n / 1_000_000:.1f}M"
    if n >= 10_000:
        return f"{n // 1000}k"
    if n >= 1000:
        return f"{n / 1000:.1f}k"
    return str(n)


def fmt_duration(seconds: float) -> str:
    sec = max(0, int(seconds))
    if sec < 60:
        return f"{sec}s"
    mins, rem = divmod(sec, 60)
    if mins < 60:
        return f"{mins}m{rem:02d}s" if rem else f"{mins}m"
    hours, mins = divmod(mins, 60)
    return f"{hours}h{mins:02d}m"


def fmt_reset(period_end: str | None) -> str:
    if not period_end:
        return ""
    try:
        end = datetime.fromisoformat(period_end.replace("Z", "+00:00"))
        if end.tzinfo is None:
            end = end.replace(tzinfo=timezone.utc)
        ms = end.timestamp() - time.time()
    except ValueError:
        return ""
    if ms <= 0:
        return "soon"
    hours = int(ms // 3600)
    if hours < 48:
        return f"{max(1, hours)}h"
    return f"{hours // 24}d"


def iso_to_epoch(raw: str | None) -> int | None:
    if not raw:
        return None
    try:
        dt = datetime.fromisoformat(raw.replace("Z", "+00:00"))
        if dt.tzinfo is None:
            dt = dt.replace(tzinfo=timezone.utc)
        return int(dt.timestamp())
    except ValueError:
        return None


def display_model(model_id: str) -> str:
    if not model_id:
        return ""
    if model_id.lower().startswith("grok-"):
        return "Grok " + model_id[5:]
    return model_id


def pct_color(pct: float) -> str:
    if pct >= 90:
        return RED
    if pct >= 70:
        return YELLOW
    return GREEN


def progress_bar(pct: float, width: int) -> str:
    width = max(4, width)
    filled = int(round(max(0.0, min(100.0, pct)) * width / 100.0))
    filled = min(width, filled)
    return "█" * filled + "░" * (width - filled)


# --- config -----------------------------------------------------------------


@dataclass
class Config:
    kind: str = "builtin"  # builtin | command
    command: str = ""
    padding: int = 0
    refresh_interval: float = 1.0
    height: int = 2
    bar_width: int = 10
    usage_enabled: bool = True
    git_enabled: bool = True
    git_cache_seconds: float = 5.0
    git_untracked: bool = False
    command_timeout: float = 0.2


def _as_int(val: Any, default: int) -> int:
    if isinstance(val, bool) or not isinstance(val, (int, float)):
        return default
    return int(val)


def _as_float(val: Any, default: float) -> float:
    if isinstance(val, bool) or not isinstance(val, (int, float)):
        return default
    return float(val)


def load_config(home: Path, cwd: str) -> Config:
    cfg = Config()
    blobs: list[Any] = []
    for path in (home / "grok-statusline.json", Path(cwd) / ".grok" / "grok-statusline.json"):
        data = read_json(path)
        if isinstance(data, dict):
            blobs.append(data)
    for data in blobs:
        sl = data.get("statusLine") if isinstance(data.get("statusLine"), dict) else data
        if isinstance(sl.get("type"), str):
            cfg.kind = sl["type"].strip().lower() or cfg.kind
        if isinstance(sl.get("command"), str):
            cfg.command = sl["command"]
        cfg.padding = max(0, _as_int(sl.get("padding"), cfg.padding))
        cfg.refresh_interval = max(0.3, _as_float(sl.get("refreshInterval"), cfg.refresh_interval))
        cfg.height = max(1, min(MAX_HEIGHT, _as_int(sl.get("height"), cfg.height)))
        cfg.bar_width = max(4, min(20, _as_int(sl.get("barWidth"), cfg.bar_width)))
        cfg.command_timeout = max(0.05, _as_float(sl.get("commandTimeout"), cfg.command_timeout))
        usage = data.get("usage") if isinstance(data.get("usage"), dict) else {}
        if isinstance(usage.get("enabled"), bool):
            cfg.usage_enabled = usage["enabled"]
        git = data.get("git") if isinstance(data.get("git"), dict) else {}
        if isinstance(git.get("enabled"), bool):
            cfg.git_enabled = git["enabled"]
        cfg.git_cache_seconds = max(1.0, _as_float(git.get("cacheSeconds"), cfg.git_cache_seconds))
        if isinstance(git.get("untracked"), bool):
            cfg.git_untracked = git["untracked"]
    return cfg


# --- session / git / billing ------------------------------------------------


def pick_session_dir(home: Path, cwd: str) -> Path | None:
    active = JSON_CACHE.get(home / "active_sessions.json")
    cwd_res = str(Path(cwd).resolve())
    candidates: list[tuple[bool, float, Path]] = []
    if isinstance(active, list):
        for entry in active:
            if not isinstance(entry, dict):
                continue
            sid = entry.get("session_id")
            ecwd = entry.get("cwd")
            if not isinstance(sid, str):
                continue
            if isinstance(ecwd, str) and str(Path(ecwd).resolve()) != cwd_res:
                continue
            d = home / "sessions" / encode_cwd(ecwd or cwd) / sid
            if not d.is_dir():
                d = _find_session(home, sid)
            if d is None:
                continue
            candidates.append(_score_session(d))
    if candidates:
        candidates.sort()
        return candidates[-1][2]
    encoded = home / "sessions" / encode_cwd(cwd)
    if encoded.is_dir():
        kids = [_score_session(child) for child in encoded.iterdir() if child.is_dir()]
        if kids:
            kids.sort()
            return kids[-1][2]
    return None


def _score_session(d: Path) -> tuple[bool, float, Path]:
    has_ctx = (d / "signals.json").is_file() or (d / "updates.jsonl").is_file()
    try:
        mtime = next(
            (d / name).stat().st_mtime
            for name in ("signals.json", "updates.jsonl", "summary.json")
            if (d / name).is_file()
        )
    except (OSError, StopIteration):
        mtime = d.stat().st_mtime
    return has_ctx, mtime, d


def _find_session(home: Path, session_id: str) -> Path | None:
    root = home / "sessions"
    if not root.is_dir():
        return None
    try:
        for cwd_enc in root.iterdir():
            cand = cwd_enc / session_id
            if cand.is_dir():
                return cand
    except OSError:
        return None
    return None


def _tokens_from_updates(path: Path) -> int:
    try:
        size = path.stat().st_size
    except OSError:
        return 0
    max_bytes = min(size, 64 * 1024)
    try:
        with path.open("rb") as fh:
            fh.seek(max(0, size - max_bytes))
            tail = fh.read().decode("utf8", "replace")
    except OSError:
        return 0
    best = 0
    for line in tail.splitlines():
        if "totalTokens" not in line:
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        n = obj.get("params", {}).get("_meta", {}).get("totalTokens")
        if isinstance(n, (int, float)) and n > best:
            best = int(n)
    return best


@dataclass
class GitInfo:
    branch: str = ""
    staged: int = 0
    modified: int = 0
    untracked: int = 0
    ahead: int = 0
    behind: int = 0
    origin_url: str = ""


_GIT: dict[str, tuple[float, GitInfo]] = {}


def git_info(cwd: str, cache_seconds: float, untracked: bool) -> GitInfo:
    now = time.monotonic()
    hit = _GIT.get(cwd)
    if hit and now - hit[0] < cache_seconds:
        return hit[1]
    info = _git_probe(cwd, untracked)
    _GIT[cwd] = (now, info)
    return info


def _git_probe(cwd: str, untracked: bool) -> GitInfo:
    git = shutil.which("git")
    if not git:
        return GitInfo()
    env = {**os.environ, "GIT_OPTIONAL_LOCKS": "0"}
    args = [git, "-C", cwd, "status", "--porcelain=v1", "-b"]
    if not untracked:
        args.append("--untracked-files=no")
    try:
        proc = subprocess.run(args, capture_output=True, text=True, timeout=0.4, env=env)
    except (OSError, subprocess.TimeoutExpired):
        return GitInfo()
    if proc.returncode != 0:
        return GitInfo()
    info = GitInfo()
    lines = proc.stdout.splitlines()
    if lines and lines[0].startswith("## "):
        head = lines[0][3:]
        name = head.split("...")[0].strip()
        info.branch = "" if name == "HEAD (no branch)" else name
        m = re.search(r"\[(?:ahead (\d+))?(?:, )?(?:behind (\d+))?\]", head)
        if m:
            info.ahead = int(m.group(1) or 0)
            info.behind = int(m.group(2) or 0)
        rest = lines[1:]
    else:
        rest = lines
    for line in rest:
        if len(line) < 2:
            continue
        x, y = line[0], line[1]
        if x == "?" and y == "?":
            info.untracked += 1
            continue
        if x not in " ?":
            info.staged += 1
        if y not in " ?":
            info.modified += 1
    try:
        rem = subprocess.run(
            [git, "-C", cwd, "remote", "get-url", "origin"],
            capture_output=True,
            text=True,
            timeout=0.3,
            env=env,
        )
        if rem.returncode == 0:
            info.origin_url = rem.stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        pass
    return info


def parse_origin(url: str) -> dict[str, str] | None:
    if not url:
        return None
    raw = url.strip()
    if raw.startswith("git@"):
        # git@host:owner/name.git
        try:
            host, path = raw.split(":", 1)
            host = host.split("@", 1)[1]
        except ValueError:
            return None
    else:
        parsed = urlparse(raw)
        host = parsed.hostname or ""
        path = parsed.path.lstrip("/")
    path = path.removesuffix(".git")
    parts = [p for p in path.split("/") if p]
    if not host or len(parts) < 2:
        return None
    return {"host": host, "owner": parts[-2], "name": parts[-1]}


def _num(obj: Any) -> float | None:
    if isinstance(obj, (int, float)) and not isinstance(obj, bool):
        return float(obj)
    if isinstance(obj, dict) and isinstance(obj.get("val"), (int, float)):
        return float(obj["val"])
    return None


def _auth_token(home: Path) -> str | None:
    raw = read_json(home / "auth.json")
    if not isinstance(raw, dict):
        return None
    for entry in raw.values():
        if not isinstance(entry, dict):
            continue
        for key in ("key", "access_token"):
            val = entry.get(key)
            if isinstance(val, str) and len(val) > 10:
                return val
    return None


def _http_json(url: str, token: str) -> Any | None:
    req = urllib.request.Request(
        url,
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/json",
            "User-Agent": f"grok-statusline/{VERSION}",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=8) as resp:
            return json.loads(resp.read().decode("utf8"))
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, OSError):
        return None


def fetch_usage(home: Path) -> dict[str, Any] | None:
    cache = home / "grok-statusline" / "billing-cache.json"
    try:
        cached = json.loads(cache.read_text(encoding="utf8"))
        age = time.time() - cached.get("fetched_at", 0)
        if 0 <= age < 60 and isinstance(cached.get("data"), dict):
            return cached["data"]
    except (OSError, json.JSONDecodeError, TypeError):
        cached = None

    token = _auth_token(home)
    if not token:
        return cached["data"] if isinstance(cached, dict) else None

    credits = _http_json(CREDITS_URL, token)
    monthly = _http_json(MONTHLY_URL, token)
    data: dict[str, Any] = {}

    if isinstance(credits, dict):
        cfg = credits.get("config") if isinstance(credits.get("config"), dict) else credits
        pct = cfg.get("creditUsagePercent")
        if not isinstance(pct, (int, float)) and isinstance(cfg.get("productUsage"), list) and cfg["productUsage"]:
            first = cfg["productUsage"][0]
            pct = first.get("usagePercent") if isinstance(first, dict) else None
        if isinstance(pct, (int, float)):
            data["percent"] = max(0.0, min(100.0, float(pct)))
            data["periodType"] = "weekly"
        period = cfg.get("currentPeriod") if isinstance(cfg.get("currentPeriod"), dict) else {}
        if isinstance(period.get("end"), str):
            data["periodEnd"] = period["end"]
            data["weekEnd"] = period["end"]

    if "percent" not in data and isinstance(monthly, dict):
        mcfg = monthly.get("config") if isinstance(monthly.get("config"), dict) else monthly
        used = _num(mcfg.get("used"))
        limit = _num(mcfg.get("monthlyLimit"))
        if used is not None and limit and limit > 0:
            data["percent"] = max(0.0, min(100.0, 100.0 * used / limit))
            data["periodType"] = "monthly"
            if isinstance(mcfg.get("billingPeriodEnd"), str):
                data["periodEnd"] = mcfg["billingPeriodEnd"]

    if not data:
        return cached["data"] if isinstance(cached, dict) else None

    try:
        cache.parent.mkdir(parents=True, exist_ok=True)
        cache.write_text(json.dumps({"fetched_at": time.time(), "data": data}) + "\n", encoding="utf8")
    except OSError:
        pass
    return data


# --- Claude-shaped payload --------------------------------------------------


def build_payload(home: Path, cwd: str, cfg: Config, usage: dict[str, Any] | None) -> dict[str, Any]:
    session_dir = pick_session_dir(home, cwd)
    signals = JSON_CACHE.get(session_dir / "signals.json") if session_dir else None
    summary = JSON_CACHE.get(session_dir / "summary.json") if session_dir else None
    signals = signals if isinstance(signals, dict) else {}
    summary = summary if isinstance(summary, dict) else {}

    model_id = ""
    if isinstance(summary.get("current_model_id"), str):
        model_id = summary["current_model_id"]
    elif isinstance(signals.get("primaryModelId"), str):
        model_id = signals["primaryModelId"]

    used = signals.get("contextTokensUsed")
    window = signals.get("contextWindowTokens")
    pct = signals.get("contextWindowUsage")
    ctx_used = int(used) if isinstance(used, (int, float)) else 0
    ctx_window = int(window) if isinstance(window, (int, float)) and window > 0 else 0
    ctx_pct = float(pct) if isinstance(pct, (int, float)) else 0.0
    if ctx_used == 0 and session_dir:
        est = _tokens_from_updates(session_dir / "updates.jsonl")
        if est:
            ctx_used = est
            if ctx_window <= 0:
                ctx_window = DEFAULT_WINDOW
    if ctx_window <= 0 and ctx_used:
        ctx_window = DEFAULT_WINDOW
    if ctx_window > 0 and ctx_pct <= 0 and ctx_used:
        ctx_pct = 100.0 * ctx_used / ctx_window

    info = summary.get("info") if isinstance(summary.get("info"), dict) else {}
    session_id = info.get("id") if isinstance(info.get("id"), str) else ""
    if not session_id and session_dir:
        session_id = session_dir.name

    # Prefer wall-clock from created_at so the bar ticks while signals.json is idle.
    created = summary.get("created_at")
    start = iso_to_epoch(created if isinstance(created, str) else None)
    if start:
        duration_s = max(0.0, time.time() - start)
    else:
        raw_dur = signals.get("sessionDurationSeconds")
        duration_s = float(raw_dur) if isinstance(raw_dur, (int, float)) else 0.0
    added = signals.get("agentLinesAdded")
    removed = signals.get("agentLinesRemoved")

    g = git_info(cwd, cfg.git_cache_seconds, cfg.git_untracked) if cfg.git_enabled else GitInfo()
    repo = parse_origin(g.origin_url)

    payload: dict[str, Any] = {
        "cwd": cwd,
        "session_id": session_id,
        "transcript_path": str(session_dir / "updates.jsonl") if session_dir else "",
        "version": VERSION,
        "model": {"id": model_id, "display_name": display_model(model_id) or model_id},
        "workspace": {
            "current_dir": cwd,
            "project_dir": cwd,
        },
        "cost": {
            "total_duration_ms": int(float(duration_s) * 1000),
            "total_lines_added": int(added) if isinstance(added, (int, float)) else 0,
            "total_lines_removed": int(removed) if isinstance(removed, (int, float)) else 0,
        },
        "context_window": {
            "total_input_tokens": ctx_used,
            "total_output_tokens": 0,
            "context_window_size": ctx_window,
            "used_percentage": ctx_pct,
            "remaining_percentage": max(0.0, 100.0 - ctx_pct),
        },
        "exceeds_200k_tokens": ctx_used > 200_000,
        "git": {
            "branch": g.branch,
            "staged": g.staged,
            "modified": g.modified,
            "untracked": g.untracked,
            "ahead": g.ahead,
            "behind": g.behind,
        },
    }
    title = summary.get("generated_title") or summary.get("session_summary")
    if isinstance(title, str) and title:
        payload["session_name"] = title
    effort = summary.get("reasoning_effort")
    if isinstance(effort, str) and effort:
        payload["effort"] = {"level": effort}
    agent = summary.get("agent_name")
    if isinstance(agent, str) and agent:
        payload["agent"] = {"name": agent}
    if repo:
        payload["workspace"]["repo"] = repo
    turns = signals.get("turnCount")
    tools = signals.get("toolCallCount")
    compact = signals.get("compactionCount")
    extra: dict[str, Any] = {}
    if isinstance(turns, (int, float)):
        extra["turn_count"] = int(turns)
    if isinstance(tools, (int, float)):
        extra["tool_call_count"] = int(tools)
    if isinstance(compact, (int, float)):
        extra["compaction_count"] = int(compact)
    if extra:
        payload["grok"] = extra

    if usage and isinstance(usage.get("percent"), (int, float)):
        period = str(usage.get("periodType") or "weekly")
        key = "seven_day" if period == "weekly" else "five_hour"
        window_rl: dict[str, Any] = {"used_percentage": float(usage["percent"])}
        resets = iso_to_epoch(usage.get("periodEnd") or usage.get("weekEnd"))
        if resets:
            window_rl["resets_at"] = resets
        payload["rate_limits"] = {key: window_rl}
        payload["usage"] = {
            "percent": float(usage["percent"]),
            "period": period,
            "reset": fmt_reset(usage.get("periodEnd") or usage.get("weekEnd")),
        }
    return payload


# --- render -----------------------------------------------------------------


def render_builtin(payload: dict[str, Any], cols: int, cfg: Config) -> list[str]:
    model = payload.get("model", {}).get("display_name") or payload.get("model", {}).get("id") or "grok"
    cwd = payload.get("workspace", {}).get("current_dir") or payload.get("cwd") or ""
    folder = Path(cwd).name or cwd
    git = payload.get("git") if isinstance(payload.get("git"), dict) else {}
    branch = str(git.get("branch") or "")
    staged = int(git.get("staged") or 0)
    modified = int(git.get("modified") or 0)
    effort = ""
    if isinstance(payload.get("effort"), dict):
        effort = str(payload["effort"].get("level") or "")

    left: list[str] = [f"{CYAN}[{model}]{FG}", folder]
    if branch:
        dirty = ""
        if staged:
            dirty += f"{GREEN}+{staged}{FG}"
        if modified:
            dirty += f"{YELLOW}~{modified}{FG}"
        left.append(f"{branch}{(' ' + dirty) if dirty else ''}")
    if effort:
        left.append(f"{DIM}{effort}{FG}")
    line1 = f"{DIM}  {FG}".join(left)

    ctx = payload.get("context_window") if isinstance(payload.get("context_window"), dict) else {}
    pct = float(ctx.get("used_percentage") or 0)
    used = int(ctx.get("total_input_tokens") or 0)
    window = int(ctx.get("context_window_size") or 0)
    color = pct_color(pct)
    bar = progress_bar(pct, cfg.bar_width)
    bits = [f"{color}{bar}{FG} {color}{pct:.0f}%{FG}"]
    if window:
        bits[0] += f"{DIM} {fmt_tokens(used)}/{fmt_tokens(window)}{FG}"
    usage = payload.get("usage") if isinstance(payload.get("usage"), dict) else None
    if usage and usage.get("percent") is not None:
        up = float(usage["percent"])
        extra = str(usage.get("period") or "")
        extra = {"weekly": "wk", "monthly": "mo"}.get(extra, extra)
        reset = str(usage.get("reset") or "")
        label = f"use {up:.0f}%"
        if extra:
            label += f" {extra}"
        if reset:
            label += f" {reset}"
        bits.append(f"{pct_color(up)}{label}{FG}")
    cost = payload.get("cost") if isinstance(payload.get("cost"), dict) else {}
    dur_ms = int(cost.get("total_duration_ms") or 0)
    if dur_ms:
        bits.append(fmt_duration(dur_ms / 1000))
    added = int(cost.get("total_lines_added") or 0)
    removed = int(cost.get("total_lines_removed") or 0)
    if added or removed:
        bits.append(f"{GREEN}+{added}{FG}{DIM}/{RED}-{removed}{FG}")
    line2 = f"{DIM} │ {FG}".join(bits)

    if cfg.height <= 1:
        compact = f"{CYAN}[{model}]{FG}{DIM} │ {FG}{color}{bar} {pct:.0f}%{FG}"
        if usage and usage.get("percent") is not None:
            compact += f"{DIM} │ {FG}{pct_color(float(usage['percent']))}use {float(usage['percent']):.0f}%{FG}"
        if branch:
            compact += f"{DIM} │ {FG}{branch}"
        return [pad_bar(compact, cols, cfg.padding)]
    return [pad_bar(line1, cols, cfg.padding), pad_bar(line2, cols, cfg.padding)]


def run_user_command(cmd: str, payload: dict[str, Any], cols: int, timeout: float) -> list[str] | None:
    env = os.environ.copy()
    env["COLUMNS"] = str(cols)
    env["LINES"] = str(MAX_HEIGHT)
    expanded = os.path.expanduser(os.path.expandvars(cmd))
    try:
        proc = subprocess.run(
            expanded,
            shell=True,
            input=json.dumps(payload, ensure_ascii=False),
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if proc.returncode != 0:
        return None
    lines = [ln.rstrip("\n") for ln in proc.stdout.splitlines()]
    if not lines:
        return None
    return lines[:MAX_HEIGHT]


def render_status(payload: dict[str, Any], cols: int, cfg: Config) -> list[str]:
    if cfg.kind == "command" and cfg.command:
        out = run_user_command(cfg.command, payload, cols, cfg.command_timeout)
        if out:
            return out
    return render_builtin(payload, cols, cfg)


# --- PTY wrap ---------------------------------------------------------------


def set_winsize(fd: int, rows: int, cols: int) -> None:
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


def paint(lines: list[str], rows: int, height: int) -> None:
    start = max(1, rows - height + 1)
    sys.stdout.write("\0337")
    for i, line in enumerate(lines[:height]):
        sys.stdout.write(f"\033[{start + i};1H\033[0m\033[2K{line}")
    for i in range(len(lines), height):
        sys.stdout.write(f"\033[{start + i};1H\033[0m\033[2K")
    sys.stdout.write("\0338")
    sys.stdout.flush()


@dataclass
class Live:
    payload: dict[str, Any] = field(default_factory=dict)
    usage: dict[str, Any] | None = None
    lines: list[str] = field(default_factory=list)
    lock: threading.Lock = field(default_factory=threading.Lock)


def run_wrap(cmd: list[str], cwd: str, cfg: Config) -> int:
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        os.execvp(cmd[0], cmd)

    home = grok_home()
    live = Live()
    height = cfg.height if cfg.kind == "builtin" else max(1, cfg.height)

    def refresh(cols: int) -> None:
        with live.lock:
            usage = live.usage
        payload = build_payload(home, cwd, cfg, usage)
        lines = render_status(payload, cols, cfg)
        with live.lock:
            live.payload = payload
            live.lines = lines

    def usage_loop(stop: threading.Event) -> None:
        while not stop.is_set():
            try:
                data = fetch_usage(home)
            except Exception:
                data = None
            with live.lock:
                live.usage = data
            stop.wait(60)

    cols, rows = shutil.get_terminal_size()
    refresh(cols)
    stop = threading.Event()
    if cfg.usage_enabled:
        threading.Thread(target=usage_loop, args=(stop,), daemon=True).start()

    child_rows = max(3, rows - height)
    pid, master = pty.fork()
    if pid == 0:
        set_winsize(sys.stdout.fileno(), child_rows, cols)
        os.execvp(cmd[0], cmd)

    set_winsize(master, child_rows, cols)
    old = termios.tcgetattr(0)
    last_data = 0.0
    last_refresh = 0.0
    last_paint_key = ""

    def on_winch(_signum: int, _frame: Any) -> None:
        nonlocal cols, rows, child_rows
        cols, rows = shutil.get_terminal_size()
        child_rows = max(3, rows - height)
        try:
            set_winsize(master, child_rows, cols)
            os.kill(pid, signal.SIGWINCH)
        except OSError:
            pass

    signal.signal(signal.SIGWINCH, on_winch)

    def do_paint(force: bool = False) -> None:
        nonlocal last_paint_key
        with live.lock:
            lines = list(live.lines)
        key = "\n".join(lines)
        if not force and key == last_paint_key:
            return
        last_paint_key = key
        paint(lines, rows, height)

    try:
        tty.setraw(0)
        while True:
            now = time.monotonic()
            if now - last_refresh > cfg.refresh_interval:
                refresh(cols)
                last_refresh = now
            try:
                readable, _, _ = select.select([0, master], [], [], 0.15)
            except InterruptedError:
                readable = []
            if 0 in readable:
                try:
                    chunk = os.read(0, 8192)
                except OSError:
                    chunk = b""
                if chunk:
                    os.write(master, chunk)
            if master in readable:
                try:
                    out = os.read(master, 65536)
                except OSError:
                    out = b""
                if not out:
                    break
                os.write(1, out)
                last_data = now
                # Child redraws can erase our rows; always restore after output.
                do_paint(force=True)
            elif now - last_data > 0.12:
                do_paint(force=False)
    finally:
        stop.set()
        termios.tcsetattr(0, termios.TCSADRAIN, old)
        sys.stdout.write(RESET)
        sys.stdout.flush()
        try:
            os.close(master)
        except OSError:
            pass
        _, status = os.waitpid(pid, 0)
        return os.waitstatus_to_exitcode(status)


def print_help() -> None:
    sys.stdout.write(
        """grok-sl — Claude-style bottom statusline for Grok

Usage:
  grok-sl [grok args...]     wrap grok; bar on the last rows
  grok-sl --once             print the bar once and exit
  grok-sl --dump-json        print the Claude-shaped session JSON
  grok-sl --no-usage         skip billing API
  grok-sl --height N         reserve N rows (1-3)
  grok-sl --help

Config: ~/.grok/grok-statusline.json  (see config.example.json)
Custom command receives the JSON on stdin, same as Claude Code.
"""
    )


def main(argv: list[str]) -> int:
    once = False
    dump_json = False
    height_override: int | None = None
    usage_override: bool | None = None
    grok_args: list[str] = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if a in ("-h", "--help"):
            print_help()
            return 0
        if a in ("-V", "--version"):
            print(VERSION)
            return 0
        if a == "--once":
            once = True
        elif a == "--dump-json":
            dump_json = True
        elif a == "--no-usage":
            usage_override = False
        elif a == "--height":
            i += 1
            if i >= len(argv):
                sys.stderr.write("grok-sl: --height needs a number\n")
                return 2
            height_override = max(1, min(MAX_HEIGHT, int(argv[i])))
        elif a.startswith("--height="):
            height_override = max(1, min(MAX_HEIGHT, int(a.split("=", 1)[1])))
        elif a == "--":
            grok_args.extend(argv[i + 1 :])
            break
        else:
            grok_args.extend(argv[i:])
            break
        i += 1

    cwd = os.getcwd()
    home = grok_home()
    cfg = load_config(home, cwd)
    if height_override is not None:
        cfg.height = height_override
    if usage_override is False or os.environ.get("GROK_SL_NO_USAGE") in {"1", "true", "yes"}:
        cfg.usage_enabled = False

    if dump_json or once:
        usage_data = fetch_usage(home) if cfg.usage_enabled else None
        payload = build_payload(home, cwd, cfg, usage_data)
        if dump_json:
            json.dump(payload, sys.stdout, ensure_ascii=False, indent=2)
            sys.stdout.write("\n")
            return 0
        cols = shutil.get_terminal_size().columns
        sys.stdout.write("\n".join(render_status(payload, cols, cfg)) + "\n")
        return 0

    grok = shutil.which("grok")
    if not grok:
        sys.stderr.write("grok-sl: grok not found on PATH\n")
        return 127
    return run_wrap([grok, *grok_args], cwd, cfg)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
