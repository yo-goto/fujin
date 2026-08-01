#!/usr/bin/env python3
# kitty を要求しないプローブ（= 実際の Claude Code と同じ状態のペイン）
import os, sys, time, termios, tty
LOG = sys.argv[1]
fd = sys.stdin.fileno()
tty.setraw(fd)
log = open(LOG, "w", buffering=1)
log.write("start (kitty要求なし)\n")
while True:
    d = os.read(fd, 4096)
    if not d:
        break
    log.write(f"{time.strftime('%H:%M:%S')} RECV {d!r}\n")
