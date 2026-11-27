# SSH connections

Run pi on other machines. pi runs **on the host**: its tools, `AGENTS.md`, skills and sessions
all live there. Pistachio reaches it through your system `ssh`, so your `~/.ssh/config`, keys,
ssh-agent and `known_hosts` apply.

## Settings

Connect saved connections at start
: Off by default. When on, saved hosts connect quietly in the background when Pistachio opens
  (no password prompts), so their status and sessions are ready. Takes effect the next time
  Pistachio opens.

Saved connections
: Each host with a status light: green (ready), amber (working or needs you), red (failed) or
  grey (not connected). Its switch connects it or turns it off. Its **⋮** menu changes the
  host, user, port, key file and jump host (kept in Pistachio; `~/.ssh/config` is not written),
  or removes it.

Connections are saved in `~/.pi/agent/pistachio/hosts.json`.

## Adding a host

In Code mode, click the device button next to the folder in the message box ("This
computer"):

- **Add SSH connection…** lists the hosts in your `~/.ssh/config` (following `Include`), or
  takes a custom host, user, port, key and jump host.
- **Manage SSH connections…** renames them, changes the command that starts pi, and removes
  them.

Then pick a folder on the host. The folder browser takes a typed or pasted path, creates folders
and can show hidden ones.

Passwords, key passphrases and one-time codes are asked for in the app; nothing is stored. An
unknown host key is shown for you to confirm. A changed host key is explained, and you can
remove the old key and confirm the new one.

On macOS, the first connection to your local network asks whether Pistachio may reach devices
there. The connection is retried for about 15 seconds, so it goes through once you allow it.

## pi on the host

If the host has no pi, Pistachio installs it, with no root access, into `~/.pistachio/server`
on the host. It uses the host's Node.js 22.19+ or a downloaded, checksum-verified standalone
Node.js. Remove that folder to undo it.

## Models on SSH hosts

The models on this computer work on every host, with nothing to set up: the model list, default
model and thinking level are the ones you have here. Model requests come back over the SSH
connection and go out from this computer, so models on `localhost`, your LAN or a VPN work from
any host, and keys and logins never leave this computer. Each request needs a token that only
works for that connection.

Logins that take more than a key (ChatGPT/Codex, GitHub Copilot, Vertex, Bedrock) work too, with
pi 1.5 or newer on the host. Custom thinking budgets and sampling parameters are not passed on
for these.

pi on the host reads a settings folder Pistachio writes there, `~/.pistachio/agent`. The host's
own `~/.pi` is not changed, and its extensions, skills, prompts and themes are linked in. If the
host doesn't allow port forwarding, pi there uses its own models, and the model menu says why.

## Remote sessions

Remote sessions behave like local ones. They are listed in the sidebar under the host (then
folder), and can be opened, renamed and moved to the host's trash. The Diff and Files panels
read the host's folder (refreshed every 10 seconds while shown and after each turn), the
terminal is your shell on the host, and `@` mentions list the host's files. When the connection
drops, the session offers to reconnect.

One shared connection per host (ssh's ControlMaster) makes later commands fast.

## Limits

- csh and tcsh login shells are not supported (bash, zsh, dash, ksh and fish are).
- Windows clients are untested. Windows' OpenSSH can't share connections, so every command opens
  its own; use keys or an agent, since a password would be asked for each time.
- The [sandbox](sandboxing-and-security.md) applies only on this computer.
