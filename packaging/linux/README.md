# KMSync Server Linux Deployment

This package contains the `kmsync-server` Linux executable plus sample deployment files.

## Files

- `bin/kmsync-server`: backend control-plane server.
- `config/kmsync-server.example.json`: configuration sample.
- `systemd/kmsync-server.service`: sample systemd unit.

## Configuration

The server reads one JSON configuration file. Pass its path as the first CLI
argument; when omitted, `/etc/kmsync/kmsync-server.json` is used.

- `bind`: bind address, for example `0.0.0.0:24888`
- `data_path`: required local JSON state path. The server stores users,
  devices, sessions, profiles, relay/signaling state, and heartbeat presence in
  this file.

## Example

```bash
sudo useradd --system --home /var/lib/kmsync --shell /usr/sbin/nologin kmsync
sudo install -d -o kmsync -g kmsync /opt/kmsync/bin /etc/kmsync /var/lib/kmsync
sudo install -m 0755 bin/kmsync-server /opt/kmsync/bin/kmsync-server
sudo install -m 0640 -o root -g kmsync config/kmsync-server.example.json /etc/kmsync/kmsync-server.json
sudo install -m 0644 systemd/kmsync-server.service /etc/systemd/system/kmsync-server.service
sudo editor /etc/kmsync/kmsync-server.json
sudo systemctl daemon-reload
sudo systemctl enable --now kmsync-server
```
