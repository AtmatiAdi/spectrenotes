# Privacy policy

SpectreNotes is a local-first, self-hosted application. **The project operates no
servers and collects no data.** There is no telemetry, no analytics, no crash
reporting and no account with the project's authors.

Everything below describes data that stays on your machine, or that leaves it
only because you explicitly asked for a feature that needs the network.

## Stored on your computer

| what | where |
|---|---|
| notes (the CRDT op-log), thumbnails, per-machine settings | `%APPDATA%\SpectreNotes` |
| GitHub access token, when you sign in | `%APPDATA%\SpectreNotes\github.token`, encrypted with Windows DPAPI — only your Windows account on that machine can read it |
| the application itself | `%LOCALAPPDATA%\SpectreNotes\app` |

Uninstalling removes the application, not your notes.

## Network connections, and what triggers them

- **Update check** — the application asks GitHub for the latest release of
  `AtmatiAdi/spectrenotes-releases` shortly after start and every 10 minutes.
  This is a plain HTTPS request for a public page; it carries no identifiers
  beyond what any HTTP client sends. Nothing is downloaded or installed without
  you clicking it.
- **Git synchronisation (optional)** — if you sign in with GitHub, notes are
  pushed to **a repository owned by you**. The authors have no access to it.
  Sign-in uses GitHub's OAuth flow; the resulting token is stored as described
  above and can be revoked at any time in your GitHub account settings.
- **Live drawing on the local network (optional, can be switched off)** — only
  notes you explicitly share are announced on your LAN, and only to peers on it.
  Content travels as plaintext TCP; the trust boundary is your own network.
  A share password never leaves your machine — it is turned into a key, and
  peers prove knowledge of it without transmitting it.
- **Feedback (only when you use it)** — the *Send feedback* window creates an
  issue **in your own name, with your own token**, in the public releases
  repository. The issue body contains what you typed plus a short technical
  footer (application version, Windows build, GPU name, window size, sync
  status). You see this text before it is sent, and you can delete the issue
  afterwards. The project's authors do not receive anything else.

## Children and sensitive data

The application is a general-purpose notebook. Whatever you write in it stays in
your files; the project never sees it.

## Contact

Questions and requests: <https://github.com/AtmatiAdi/spectrenotes/issues>
