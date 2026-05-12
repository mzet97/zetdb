# Security Policy

## Supported Versions

Apenas o release mais recente e suportado com patches de seguranca. Versoes anteriores nao recebem backports.

| Version | Supported |
|---|---|
| Latest release | :white_check_mark: |
| Older releases | :x: |

## Reporting a Vulnerability

**Do NOT open a public issue for security vulnerabilities.**

Instead, send an email to the security team:

**Email:** `Matheus.zeitune.developer@gmail.com`

### What to Include

Please include the following in your report:

1. **Description** of the vulnerability
2. **Proof of Concept** (PoC) or steps to reproduce
3. **Affected versions** (commit hash or tag)
4. **Impact assessment** (what data/systems are at risk)
5. **Logs** or screenshots, if applicable
6. **Suggested fix** (optional but appreciated)

### Response Timeline

| Stage | Target Time |
|---|---|
| Acknowledgment | 2 business days |
| Initial triage | 5 business days |
| Fix (High/Critical) | 30 days |
| Fix (Medium/Low) | 90 days |
| Public disclosure | After patch release |

### Scoring

We use CVSS v3.1 for vulnerability scoring. The severity levels are:

- **Critical** (9.0-10.0): Immediate action required
- **High** (7.0-8.9): Fix in next release
- **Medium** (4.0-6.9): Fix in upcoming release
- **Low** (0.1-3.9): Fix when convenient

## Threat Model

### In Scope

- Memory safety issues (use-after-free, buffer overflow, etc.)
- Authentication/authorization bypasses
- Data exposure or leakage
- Denial of Service via protocol abuse
- Race conditions in persistence layer

### Out of Scope

- Denial of Service via network flooding (requires infrastructure-level mitigation)
- Vulnerabilities in dependencies that have already been patched upstream
- Physical access to the server
- Social engineering attacks
- Issues requiring attacker to already have shell access

## Security Best Practices for Operators

1. **Run behind a firewall** - Do not expose ZetDB directly to the internet
2. **Use a reverse proxy** (nginx, HAProxy) for TLS termination
3. **Monitor resource usage** - Set `max_keys` and `max_connections` appropriately
4. **Regular backups** - Snapshot + AOF for disaster recovery
5. **Keep updated** - Run the latest release

## Acknowledgments

We will publicly acknowledge reporters who responsibly disclose vulnerabilities (with their consent).
