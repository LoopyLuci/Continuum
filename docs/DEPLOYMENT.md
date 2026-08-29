# Deployment Guide

## Quick Start

### Server
```bash
cargo run --release -p continuum-server -- --listen 0.0.0.0:4433 --pairing-code YOUR_CODE
```

### Client
```bash
cargo run --release -p continuum-client -- --connect SERVER_ADDR --pairing-code YOUR_CODE
```

### Relay Server
```bash
cargo run --release -p relay-server -- --listen 0.0.0.0:4434
```

## Production Deployment

### Systemd Service (Linux)

Create `/etc/systemd/system/continuum-server.service`:
```ini
[Unit]
Description=Continuum Server
After=network.target

[Service]
Type=simple
User=continuum
WorkingDirectory=/opt/continuum
ExecStart=/opt/continuum/continuum-server --listen 0.0.0.0:4433 --pairing-code YOUR_CODE
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Enable and start:
```bash
sudo systemctl enable continuum-server
sudo systemctl start continuum-server
```

### Windows Service

Use NSSM to install as a service:
```cmd
nssm install ContinuumServer "C:\Continuum\continuum-server.exe" "--listen 0.0.0.0:4433 --pairing-code YOUR_CODE"
nssm start ContinuumServer
```

### Docker

```bash
docker build -t continuum-server .
docker run -d -p 4433:4433 continuum-server --listen 0.0.0.0:4433 --pairing-code YOUR_CODE
```

## Configuration

### Environment Variables
- `CONTINUUM_LOG_LEVEL`: Set log level (trace, debug, info, warn, error)
- `CONTINUUM_PAIRING_CODE`: Override pairing code
- `CONTINUUM_LISTEN`: Override listen address

### Config Files
- Linux: `~/.config/continuum/identity.toml`
- Windows: `%APPDATA%\continuum\identity.toml`
- macOS: `~/Library/Application Support/continuum/identity.toml`

## Security Considerations

1. Use strong, unique pairing codes
2. Enable TLS in production
3. Restrict firewall access to port 4433
4. Use relay server for NAT traversal
5. Monitor audit logs regularly
6. Keep software updated

## Troubleshooting

### Connection Issues
- Verify firewall allows port 4433
- Check pairing code matches on both ends
- Verify SAS words match during pairing
- Check logs for TLS/encryption errors

### Performance Issues
- Monitor CPU usage during encoding
- Check network bandwidth
- Verify hardware encoder availability
- Review rate controller logs

### Relay Issues
- Verify relay server is reachable
- Check relay secret matches
- Verify both clients can connect to relay
