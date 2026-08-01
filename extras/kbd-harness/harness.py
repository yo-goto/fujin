#!/usr/bin/env python3
# zellij を擬似端末(pty)配下で起動し、外側端末として振る舞う検証ハーネス。
#   - master 側の出力は out.log へ
#   - inject FIFO に書いたバイト列を「キー入力」として master へ流す
# これで Shift+Enter (kitty: ESC [13;2u) を実際に打鍵したのと同じ経路で送れる。
import os, sys, pty, select, fcntl, termios, struct, signal

SESSION = sys.argv[1]
LAYOUT = sys.argv[2]
BASE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(BASE, f"{SESSION}.out.log")
FIFO = os.path.join(BASE, f"{SESSION}.inject")

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

while True:
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
    if os.waitpid(pid, os.WNOHANG)[0] == pid:
        break
out.write(b"\n--- zellij client exited ---\n")
