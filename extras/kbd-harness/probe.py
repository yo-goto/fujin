#!/usr/bin/env python3
# zellij のペイン内で動かすプローブ。
#   - 起動時に kitty キーボードプロトコルを要求する (claude と同じ CSI > 1 u)
#   - 一定間隔で CSI ? u を投げ、zellij 側が持つそのペインの kitty フラグを記録
#   - 届いたバイト列をすべてタイムスタンプ付きで記録
import os, sys, time, termios, tty, select

LOG = sys.argv[1]
fd = sys.stdin.fileno()
tty.setraw(fd)
log = open(LOG, "w", buffering=1)


def mark(msg):
    log.write(f"{time.strftime('%H:%M:%S')} {msg}\n")


os.write(1, b"\x1b[>1u")
mark("SENT CSI>1u (kitty有効化を要求)")
last_query = 0.0
while True:
    now = time.time()
    if now - last_query > 2.0:
        os.write(1, b"\x1b[?u")
        last_query = now
    r, _, _ = select.select([fd], [], [], 0.3)
    if r:
        data = os.read(fd, 4096)
        if not data:
            break
        if data == b"\x1b[?1u":
            mark("kitty=ON  (CSI?1u)")
        elif data == b"\x1b[?0u":
            mark("kitty=OFF (CSI?0u)")
        else:
            mark(f"RECV {data!r}")
