# Troubleshooting Guide

## Common Issues

### "Connection failed" or "Timeout"

**Symptoms**: Client cannot connect to server

**Solutions**:
1. Verify server is running: `cargo run -p continuum-server`
2. Check firewall allows port 4433
3. Verify IP address and port are correct
4. Check pairing code matches on both sides
5. Verify SAS words match during pairing

### "Pairing failed"

**Symptoms**: Pairing handshake fails

**Solutions**:
1. Verify pairing code is correct
2. Check both devices have same code
3. Verify SAS words match on both screens
4. If SAS words don't match, disconnect immediately
5. Generate new pairing code if compromised

### "Input injection failed"

**Symptoms**: Remote input not working

**Solutions**:
1. Verify you have permission to control the remote device
2. Check that input injector is available on this platform
3. Verify the connection is still active
4. Check logs for specific error codes

### "Clipboard sync failed"

**Symptoms**: Clipboard not syncing between devices

**Solutions**:
1. Verify clipboard sharing is enabled in settings
2. Check that both devices are paired
3. Verify clipboard data is valid UTF-8
4. Check logs for clipboard update errors

### "File transfer failed"

**Symptoms**: File transfer doesn't complete

**Solutions**:
1. Verify both devices are paired
2. Check file path is accessible
3. Verify sufficient disk space
4. Check transfer directory permissions
5. Review logs for chunk errors

### "Audio not working"

**Symptoms**: No audio during remote session

**Solutions**:
1. Verify audio capture is enabled
2. Check audio input/output devices
3. Verify audio stream is paired
4. Check for audio device conflicts

### High CPU Usage

**Symptoms**: CPU usage is very high

**Solutions**:
1. Switch to hardware encoder (nvenc, amf, vaapi, videotoolbox)
2. Lower target FPS in settings
3. Reduce video quality
4. Check for encoding loop issues
5. Profile with flamegraph

### "Relay server connection failed"

**Symptoms**: Cannot connect via relay

**Solutions**:
1. Verify relay server is running
2. Check relay secret matches
3. Verify both clients can reach relay server
4. Check firewall allows relay port (default 4434)
5. Verify session registration succeeded

## Debugging

### Enable debug logging
```bash
cargo run -p continuum-client -- --log-level debug
```

### Check logs
- Linux: `~/.local/share/continuum/logs/`
- Windows: `%APPDATA%\continuum\logs\`
- macOS: `~/Library/Application Support/continuum/logs/`

### Use debug API
```bash
cargo run -p continuum-client -- --debug 9090
# Then query: curl http://127.0.0.1:9090/api/state
```

## Getting Help

- Check documentation: `docs/`
- Review logs for error messages
- Enable debug logging
- Open an issue on GitHub with logs and reproduction steps
