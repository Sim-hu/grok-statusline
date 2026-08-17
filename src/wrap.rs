use crate::config::{grok_home, Config};
use crate::install::real_grok;
use crate::render::{render_status, RESET};
use crate::session::{build_payload, fetch_usage, Usage};
use nix::errno::Errno;
use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use nix::pty::{openpty, OpenptyResult, Winsize};
use nix::sys::signal::{sigprocmask, SigSet, SigmaskHow, Signal};
use nix::sys::termios::{cfmakeraw, tcgetattr, tcsetattr, SetArg};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{execv, fork, read, setsid, write, ForkResult, Pid};
use std::ffi::CString;
use std::io::{self, Write as _};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::io::RawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

static WINCH: AtomicBool = AtomicBool::new(false);

extern "C" fn on_winch(_: libc::c_int) {
    WINCH.store(true, Ordering::Relaxed);
}

fn term_size() -> (u16, u16) {
    let mut ws = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let r = unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) };
    if r == 0 && ws.ws_row > 0 && ws.ws_col > 0 {
        (ws.ws_col, ws.ws_row)
    } else {
        (80, 24)
    }
}

fn set_winsize(fd: RawFd, rows: u16, cols: u16) {
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe {
        libc::ioctl(fd, libc::TIOCSWINSZ, &ws);
    }
}

fn is_tty(fd: RawFd) -> bool {
    unsafe { libc::isatty(fd) == 1 }
}

fn paint(lines: &[String], rows: u16, height: u16) {
    let start = rows.saturating_sub(height).saturating_add(1).max(1);
    let mut out = String::from("\x1b7");
    for (i, line) in lines.iter().take(height as usize).enumerate() {
        out.push_str(&format!("\x1b[{};1H\x1b[0m\x1b[2K{line}", start as usize + i));
    }
    for i in lines.len()..(height as usize) {
        out.push_str(&format!("\x1b[{};1H\x1b[0m\x1b[2K", start as usize + i));
    }
    out.push_str("\x1b8");
    let _ = io::stdout().write_all(out.as_bytes());
    let _ = io::stdout().flush();
}

fn passthrough_exec(real: &std::path::Path, args: &[String]) -> i32 {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(real).args(args).exec();
    eprintln!("grok-statusline: exec {}: {err}", real.display());
    127
}

fn attach_slave(slave: OwnedFd) -> Result<(), String> {
    let _ = setsid();
    unsafe {
        libc::ioctl(slave.as_raw_fd(), libc::TIOCSCTTY, 0);
    }
    let fd = slave.as_raw_fd();
    unsafe {
        if fd != 0 {
            libc::dup2(fd, 0);
        }
        if fd != 1 {
            libc::dup2(fd, 1);
        }
        if fd != 2 {
            libc::dup2(fd, 2);
        }
        if fd > 2 {
            libc::close(fd);
        }
    }
    Ok(())
}

pub fn run_wrap(args: &[String], cfg: Config) -> i32 {
    let real = match real_grok() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("grok-statusline: {e}");
            return 127;
        }
    };
    if !is_tty(0) || !is_tty(1) {
        return passthrough_exec(&real, args);
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let home = grok_home();
    let height = cfg.height.max(1);
    let usage = Arc::new(Mutex::new(None::<Usage>));
    let stop = Arc::new(AtomicBool::new(false));
    if cfg.usage_enabled {
        let usage_t = Arc::clone(&usage);
        let stop_t = Arc::clone(&stop);
        let home_t = home.clone();
        std::thread::spawn(move || loop {
            if stop_t.load(Ordering::Relaxed) {
                break;
            }
            let got = fetch_usage(&home_t);
            if let Ok(mut g) = usage_t.lock() {
                *g = got;
            }
            for _ in 0..60 {
                if stop_t.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        });
    }

    let refresh = |cols: u16, usage: &Mutex<Option<Usage>>| {
        let u = usage.lock().ok().and_then(|g| g.clone());
        let payload = build_payload(&home, &cwd, &cfg, u.as_ref());
        render_status(&payload, cols as usize, &cfg)
    };

    let (mut cols, mut rows) = term_size();
    let mut child_rows = rows.saturating_sub(height).max(3);
    let mut lines = refresh(cols, &usage);

    let ws = Winsize {
        ws_row: child_rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let OpenptyResult { master, slave } = match openpty(Some(&ws), None) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("grok-statusline: openpty: {e}");
            return passthrough_exec(&real, args);
        }
    };

    match unsafe { fork() } {
        Ok(ForkResult::Child) => {
            drop(master);
            let _ = attach_slave(slave);
            set_winsize(1, child_rows, cols);
            let bin = CString::new(real.to_string_lossy().as_bytes()).unwrap();
            let mut cargs = vec![bin.clone()];
            for a in args {
                if let Ok(c) = CString::new(a.as_str()) {
                    cargs.push(c);
                }
            }
            let _ = execv(&bin, &cargs);
            libc_exit(127);
        }
        Ok(ForkResult::Parent { child }) => {
            drop(slave);
            parent_loop(
                master,
                child,
                &usage,
                &stop,
                &mut lines,
                &mut cols,
                &mut rows,
                &mut child_rows,
                height,
                refresh,
            )
        }
        Err(e) => {
            eprintln!("grok-statusline: fork: {e}");
            passthrough_exec(&real, args)
        }
    }
}

fn libc_exit(code: i32) -> ! {
    unsafe { libc::_exit(code) }
}

fn parent_loop(
    master: OwnedFd,
    child: Pid,
    usage: &Mutex<Option<Usage>>,
    stop: &AtomicBool,
    lines: &mut Vec<String>,
    cols: &mut u16,
    rows: &mut u16,
    child_rows: &mut u16,
    height: u16,
    refresh: impl Fn(u16, &Mutex<Option<Usage>>) -> Vec<String>,
) -> i32 {
    let orig = match tcgetattr(io::stdin()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("grok-statusline: tcgetattr: {e}");
            return 1;
        }
    };
    let mut raw = orig.clone();
    cfmakeraw(&mut raw);
    let _ = tcsetattr(&io::stdin(), SetArg::TCSANOW, &raw);

    let handler = nix::sys::signal::SigHandler::Handler(on_winch);
    let _ = unsafe { nix::sys::signal::signal(Signal::SIGWINCH, handler) };
    // Keep SIGWINCH unblocked so the handler can fire during poll.
    let mut mask = SigSet::empty();
    mask.add(Signal::SIGWINCH);
    let _ = sigprocmask(SigmaskHow::SIG_UNBLOCK, Some(&mask), None);

    let mut last_refresh = Instant::now();
    let mut last_data = Instant::now();
    let mut last_key = String::new();
    let mut buf = [0u8; 65536];
    let refresh_every = std::time::Duration::from_millis(1000);

    let result = loop {
        if WINCH.swap(false, Ordering::Relaxed) {
            let (c, r) = term_size();
            *cols = c;
            *rows = r;
            *child_rows = r.saturating_sub(height).max(3);
            set_winsize(master.as_raw_fd(), *child_rows, *cols);
            let _ = nix::sys::signal::kill(child, Signal::SIGWINCH);
            *lines = refresh(*cols, usage);
            last_key.clear();
        }
        if last_refresh.elapsed() >= refresh_every {
            *lines = refresh(*cols, usage);
            last_refresh = Instant::now();
        }

        let stdin_fd = io::stdin();
        let mut fds = [
            PollFd::new(stdin_fd.as_fd(), PollFlags::POLLIN),
            PollFd::new(master.as_fd(), PollFlags::POLLIN),
        ];
        match poll(&mut fds, PollTimeout::from(150u8)) {
            Ok(_) => {}
            Err(Errno::EINTR) => continue,
            Err(_) => break 1,
        }

        if fds[0]
            .revents()
            .is_some_and(|r| r.intersects(PollFlags::POLLIN))
        {
            match read_fd(stdin_fd.as_fd(), &mut buf) {
                Ok(0) => {}
                Ok(n) => {
                    let _ = write_all_fd(master.as_fd(), &buf[..n]);
                }
                Err(Errno::EIO | Errno::EAGAIN) => {}
                Err(_) => break 1,
            }
        }
        if fds[1]
            .revents()
            .is_some_and(|r| r.intersects(PollFlags::POLLIN | PollFlags::POLLHUP))
        {
            match read_fd(master.as_fd(), &mut buf) {
                Ok(0) => break wait_code(child),
                Ok(n) => {
                    let _ = io::stdout().write_all(&buf[..n]);
                    let _ = io::stdout().flush();
                    last_data = Instant::now();
                    paint(lines, *rows, height);
                    last_key.clear();
                }
                Err(Errno::EIO) => break wait_code(child),
                Err(Errno::EAGAIN) => {}
                Err(_) => break 1,
            }
        } else if last_data.elapsed().as_millis() > 120 {
            let key = lines.join("\n");
            if key != last_key {
                last_key = key;
                paint(lines, *rows, height);
            }
        }

        if let Ok(WaitStatus::Exited(_, code)) = waitpid(child, Some(WaitPidFlag::WNOHANG)) {
            break code;
        }
        if let Ok(WaitStatus::Signaled(_, sig, _)) = waitpid(child, Some(WaitPidFlag::WNOHANG)) {
            break 128 + sig as i32;
        }
    };

    stop.store(true, Ordering::Relaxed);
    let _ = tcsetattr(&io::stdin(), SetArg::TCSANOW, &orig);
    let _ = io::stdout().write_all(RESET.as_bytes());
    let _ = io::stdout().flush();
    result
}

fn read_fd(fd: BorrowedFd<'_>, buf: &mut [u8]) -> nix::Result<usize> {
    read(fd.as_raw_fd(), buf)
}

fn write_all_fd(fd: impl AsFd, mut buf: &[u8]) -> nix::Result<()> {
    while !buf.is_empty() {
        match write(fd.as_fd(), buf) {
            Ok(0) => break,
            Ok(n) => buf = &buf[n..],
            Err(Errno::EAGAIN | Errno::EINTR) => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn wait_code(child: Pid) -> i32 {
    match waitpid(child, None) {
        Ok(WaitStatus::Exited(_, code)) => code,
        Ok(WaitStatus::Signaled(_, sig, _)) => 128 + sig as i32,
        _ => 1,
    }
}
