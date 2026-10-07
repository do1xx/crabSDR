#!/usr/bin/env python3
"""iqrec-remote — läuft per `ssh host python3 - PORT SEK` auf einem Rechner mit rtl_tcp: hängt sich als Mithörer an einen
rtl_tcp-Port (oder Verteiler), schickt KEINEN Befehl, liest SEK Sekunden u8-IQ (2,048 MS/s) in den RAM und schreibt sie nach stdout.
Kein Schreibzugriff auf dem Rechner, keine Störung des Betriebs. Aufruf:
   ssh root@mein-sdr python3 - 1234 60 < tools/phase0/iqrec-remote.py > testdata/70cm-o.cu8"""
import socket, sys, time
port, sec = int(sys.argv[1]), float(sys.argv[2])
n = int(2048000 * 2 * sec)
s = socket.create_connection(("127.0.0.1", port), timeout=10)
hdr = b""
while len(hdr) < 12:
    hdr += s.recv(12 - len(hdr))
buf = bytearray(n); mv = memoryview(buf); got = 0; t0 = time.time()
while got < n:
    k = s.recv_into(mv[got:], min(1 << 20, n - got))
    if not k:
        break
    got += k
s.close()
sys.stderr.write(f"port {port}: {got} Bytes in {time.time()-t0:.1f} s, header={hdr[:4]!r}\n")
sys.stdout.buffer.write(mv[:got])
