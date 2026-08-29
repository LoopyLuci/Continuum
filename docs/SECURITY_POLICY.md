# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.2.x   | :white_check_mark: |
| < 0.2   | :x:                |

## Reporting a Vulnerability

Please report security vulnerabilities to **security@example.com** with:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

We will acknowledge within 7 days and provide a detailed response within 30 days.

## Security Best Practices

- Always use TLS for remote connections
- Verify SAS words during pairing
- Never share pairing codes publicly
- Keep software up to date
- Use firewall rules to restrict access
- Monitor audit logs for suspicious activity

## Known Security Considerations

- Resume tokens expire after 1 hour
- PAKE mode provides forward secrecy
- Clipboard and file transfers require paired sessions
- Input injection requires explicit permission
- Debug API binds to localhost only by default
