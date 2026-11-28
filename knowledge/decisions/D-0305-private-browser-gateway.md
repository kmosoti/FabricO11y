# D-0305: Private browser query gateway

The optional browser path uses a separately installed same-user Unix gateway
for bounded typed run-list and run-summary reads. The gateway provisions one
canonical authority and a source allowlist and uses an authenticated loopback
canonical client. Kernel peer UID and private socket permissions bind its local
peer; they do not distinguish processes with the same UID or authenticate a
browser holder. An owner-managed same-origin HTTPS proxy must authenticate that
holder and forward only authorized requests to the socket.

This local profile avoids exposing canonical credentials to browser code and
keeps renderer dependencies out of headless package graphs. It does not claim
session lifecycle, remotely deployed proxy behavior, browser qualification,
or proof that a dropped HTTP request has stopped backend work. Revisit the
boundary before adding session management, streaming, writes, or remote access.
