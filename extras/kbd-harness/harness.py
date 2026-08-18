#!/usr/bin/env python3
# zellij を擬似端末(pty)配下で起動し、外側端末として振る舞う検証ハーネス。
#   - master 側の出力は out.log へ
#   - inject FIFO に書いたバイト列を「キー入力」として master へ流す
# これで Shift+Enter (kitty: ESC [13;2u) を実際に打鍵したのと同じ経路で送れる。
#
# **後始末は必ず通す（2026-08-18 に追加）。** このハーネスは
# `python3 harness.py <session> <layout> &` のようにバックグラウンドで起動される
# 想定なので、呼び出し元のシェルが畳まれると親だけが先に死ぬ。そのとき master が
# 閉じると、子の zellij クライアントは **tty を失ったまま終了処理へ入り**、
# マウスモード無効化の書き込みで EIO を unwrap して SIGABRT で落ちる
# （zellij 0.44.3 のバグ。配下のシェルが孤児化し、閉じた pty に対する
# select/read の空振りで CPU を食い尽くす)。
# → docs/issues/window-close-panics-orphan-shell.md
#
# したがって終了経路は一本に集約し、**クライアントを先に終わらせてから
# master を閉じる**順序を必ず守る。子は setsid() 済みで親のプロセスグループから
# 独立しているため、親の SIGHUP は届かない。明示的に殺さないと生き残る。
import atexit
import fcntl
import os
import pty
import select
import signal
import struct
import subprocess
import sys
import termios
import time

if len(sys.argv) < 3:
    sys.exit("usage: harness.py <session> <layout>")

SESSION = sys.argv[1]
LAYOUT = sys.argv[2]
# ログとFIFOはカレントディレクトリに作る（リポジトリを汚さないため）
BASE = os.getcwd()
OUT = os.path.join(BASE, f"{SESSION}.out.log")
FIFO = os.path.join(BASE, f"{SESSION}.inject")

# クライアントに終了処理をさせるための猶予。zellij はセッション終了時に
# レイアウトのシリアライズ等を行うので、SIGTERM を撃つ前にここまで待つ
GRACE_KILL_SESSION = 5.0
GRACE_SIGTERM = 3.0

if os.path.exists(FIFO):
    os.unlink(FIFO)
os.mkfifo(FIFO)

master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 160, 0, 0))

pid = os.fork()
if pid == 0:
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    os.dup2(slave, 0)
    os.dup2(slave, 1)
    os.dup2(slave, 2)
    os.close(master)
    os.close(slave)
    env = dict(os.environ)
    env["TERM"] = "xterm-256color"
    for k in ("ZELLIJ", "ZELLIJ_SESSION_NAME", "ZELLIJ_PANE_ID"):
        env.pop(k, None)
    os.execvpe("zellij", ["zellij", "-s", SESSION, "-n", LAYOUT], env)

os.close(slave)
out = open(OUT, "wb", buffering=0)
# FIFO は読み書き両方で開いて、書き手が居ない間に EOF にならないようにする
fifo_fd = os.open(FIFO, os.O_RDWR | os.O_NONBLOCK)

_stop = False
_shutdown_done = False
_reaped = False


def _request_stop(signum, _frame):
    # ハンドラ内では旗を立てるだけにする。select のタイムアウトは 1 秒なので
    # 遅くとも 1 秒後のループ先頭で拾える
    global _stop
    _stop = True


for _sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
    signal.signal(_sig, _request_stop)


def _reap():
    """子を回収できたら True。回収済みなら以降は常に True を返す。"""
    global _reaped
    if _reaped:
        return True
    try:
        if os.waitpid(pid, os.WNOHANG)[0] == pid:
            _reaped = True
    except ChildProcessError:
        _reaped = True
    return _reaped


def _drain(deadline):
    """クライアントが終了処理を書き切れるよう、待っている間も master を読み続ける。

    読まずに待つと相手が write でブロックして、いつまでも終わらない。
    子が終了したら True を返す。
    """
    while time.time() < deadline:
        r, _, _ = select.select([master], [], [], 0.2)
        if r:
            try:
                data = os.read(master, 65536)
            except OSError:
                data = b""
            if data:
                out.write(data)
        if _reap():
            return True
    return _reap()


def shutdown(reason="requested"):
    """クライアントを終わらせてから master を閉じる。冪等。"""
    global _shutdown_done
    if _shutdown_done:
        return
    _shutdown_done = True

    how = reason
    if not _reap():
        # ①まずセッションごと畳ませる。これが一番きれいで、クライアントは
        #   tty が生きているうちに自分で終了処理を終えられる
        try:
            subprocess.run(
                ["zellij", "kill-session", SESSION],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=GRACE_KILL_SESSION,
            )
        except Exception:
            pass
        if not _drain(time.time() + GRACE_KILL_SESSION):
            # ②それでも残るなら SIGTERM。子は setsid() 済みなのでプロセスグループごと
            how = f"{reason}+SIGTERM"
            try:
                os.killpg(pid, signal.SIGTERM)
            except OSError:
                pass
            if not _drain(time.time() + GRACE_SIGTERM):
                # ③最後の手段
                how = f"{reason}+SIGKILL"
                try:
                    os.killpg(pid, signal.SIGKILL)
                except OSError:
                    pass
                _drain(time.time() + 1.0)

    # **master を閉じるのはここまで来てから。** 逆順にすると、まさに直そうとしている
    # 「tty が消えた状態でクライアントが終了処理をする」状況を自分で作ってしまう
    try:
        out.write(f"\n--- zellij client exited ({how}) ---\n".encode())
    except Exception:
        pass
    for fd in (master, fifo_fd):
        try:
            os.close(fd)
        except OSError:
            pass
    try:
        out.close()
    except Exception:
        pass
    try:
        os.unlink(FIFO)
    except OSError:
        pass


atexit.register(shutdown, "atexit")

try:
    while not _stop:
        r, _, _ = select.select([master, fifo_fd], [], [], 1.0)
        if master in r:
            try:
                data = os.read(master, 65536)
            except OSError:
                break
            if not data:
                break
            out.write(data)
        if fifo_fd in r:
            try:
                data = os.read(fifo_fd, 4096)
            except OSError:
                data = b""
            if data:
                os.write(master, data)
        if _reap():
            break
finally:
    # 正常終了・例外・シグナルのどの経路でも必ずここを通す
    shutdown("signal" if _stop else "eof")
