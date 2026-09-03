#!/bin/bash
set -Eeuo pipefail

exec 3<>/dev/tcp/127.0.0.1/3000
printf 'GET /api/health HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n' >&3
grep -q '"status":"ok"' <&3
