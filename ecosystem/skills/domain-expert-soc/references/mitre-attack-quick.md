# MITRE ATT&CK — Quick Reference

## Top techniques by tactic (the ones you'll see weekly)

### TA0001 Initial Access
- **T1566 Phishing** — most common entry; sub-techniques: .001 attachment, .002 link, .003 service.
- **T1190 Exploit Public-Facing Application** — RCE on internet-exposed app (Log4Shell era, Confluence CVEs).
- **T1133 External Remote Services** — exposed RDP, VPN with leaked creds.
- **T1078 Valid Accounts** — credential stuffing, password spray, leaked dumps.
- **T1195 Supply Chain Compromise** — XZ utils 2024, SolarWinds 2020.

### TA0002 Execution
- **T1059 Command and Scripting Interpreter** — .001 PowerShell, .003 Windows cmd, .004 Unix shell, .005 VBScript, .006 Python, .007 JavaScript.
- **T1053 Scheduled Task/Job** — .005 schtasks, .003 cron.
- **T1569 System Services** — .002 service execution (often via psexec).
- **T1106 Native API** — direct syscall use to evade userland EDR hooks.
- **T1204 User Execution** — .001 link, .002 attachment, .003 malicious image.

### TA0003 Persistence
- **T1547 Boot or Logon Autostart Execution** — .001 Run key, .005 SSP, .009 shortcut modification.
- **T1543 Create or Modify System Process** — .003 Windows service, .001 launchd, .002 systemd.
- **T1136 Create Account** — .001 local, .002 domain, .003 cloud.
- **T1098 Account Manipulation** — adding SSH keys, adding cloud roles.
- **T1505 Server Software Component** — .003 web shell, .004 IIS module.

### TA0004 Privilege Escalation
- **T1068 Exploitation for Privilege Escalation** — kernel exploit, sudo CVE.
- **T1134 Access Token Manipulation** — .001 token impersonation.
- **T1055 Process Injection** — .001 DLL injection, .003 thread hollowing.
- **T1574 Hijack Execution Flow** — .001 DLL search-order hijacking.
- **T1078.004 Valid Accounts: Cloud Accounts** — assume-role to higher-privilege.

### TA0005 Defense Evasion
- **T1027 Obfuscated Files or Information** — packing, encoding.
- **T1140 Deobfuscate/Decode Files or Information**.
- **T1070 Indicator Removal** — .001 clear Windows event logs, .002 clear Linux history, .004 file deletion.
- **T1562 Impair Defenses** — .001 disable security tools, .004 disable firewall.
- **T1218 System Binary Proxy Execution** — .011 rundll32, .010 regsvr32, .005 mshta — LOLBins.
- **T1620 Reflective Code Loading** — in-memory load, no disk write.

### TA0006 Credential Access
- **T1003 OS Credential Dumping** — .001 LSASS, .002 SAM, .003 NTDS, .008 /etc/passwd & /etc/shadow.
- **T1110 Brute Force** — .001 password guessing, .003 password spraying.
- **T1558 Steal or Forge Kerberos Tickets** — .003 Kerberoasting, .004 AS-REP roasting.
- **T1555 Credentials from Password Stores** — .003 browser, .005 password manager.
- **T1212 Exploitation for Credential Access** — Zerologon.

### TA0007 Discovery
- **T1087 Account Discovery** — .002 domain accounts (`net user /domain`).
- **T1018 Remote System Discovery** — `net view`, AD ping sweep.
- **T1083 File and Directory Discovery**.
- **T1057 Process Discovery** — `tasklist`, `ps`.
- **T1069 Permission Groups Discovery** — `net group "Domain Admins"`.
- **T1482 Domain Trust Discovery**.

### TA0008 Lateral Movement
- **T1021 Remote Services** — .001 RDP, .002 SMB/Admin Shares, .004 SSH, .006 WinRM.
- **T1570 Lateral Tool Transfer** — copying binaries via SMB.
- **T1550 Use Alternate Authentication Material** — .002 pass the hash, .003 pass the ticket.
- **T1563 Remote Service Session Hijacking**.

### TA0009 Collection
- **T1560 Archive Collected Data** — `7z a -p`.
- **T1056 Input Capture** — .001 keylogging.
- **T1113 Screen Capture**.
- **T1005 Data from Local System**.
- **T1530 Data from Cloud Storage Object**.

### TA0011 C2
- **T1071 Application Layer Protocol** — .001 HTTPS beacon, .004 DNS tunneling.
- **T1090 Proxy** — .003 multi-hop, .002 external proxy.
- **T1573 Encrypted Channel** — .001 symmetric, .002 asymmetric.
- **T1568 Dynamic Resolution** — .002 domain generation algorithm (DGA).

### TA0010 Exfiltration
- **T1041 Exfiltration Over C2 Channel**.
- **T1567 Exfiltration Over Web Service** — .002 cloud storage.
- **T1048 Exfiltration Over Alternative Protocol** — .003 unencrypted non-C2 protocol.
- **T1029 Scheduled Transfer**.

### TA0040 Impact
- **T1486 Data Encrypted for Impact** — ransomware.
- **T1490 Inhibit System Recovery** — `vssadmin delete shadows`.
- **T1485 Data Destruction** — wiper.
- **T1499 Endpoint Denial of Service**.
- **T1496 Resource Hijacking** — cryptojacking.

## Threat-actor archetype mapping

| Actor type | Typical chain |
|---|---|
| Commodity ransomware (Lockbit-affil) | T1190/T1566 → T1059.001 → T1003.001 → T1021.002 → T1486 + T1490 |
| Living-off-the-land APT (Volt Typhoon-style) | T1190 → T1133 → T1078 → T1059.003 → T1003 → T1021.001 (long dwell, no malware) |
| BEC (no malware, social only) | T1566.002 → T1078.004 (cloud account) → T1098 mail forwarding rule → T1114 email collection |
| Insider data theft | T1078 → T1083 file discovery → T1560 archive → T1567.002 personal-cloud upload |
| Supply-chain (xz-utils style) | T1195.002 compromise dev → T1554 backdoored dependency → T1190 on victim's prod |

## Useful command line for ATT&CK mapping

- `MITRE ATT&CK Navigator` (free, hosted): map your detections + your gaps onto the matrix.
- `attack-navigator-cli`: scripted layer generation.
- `pyATT&CK` library: programmatic queries (techniques by tactic, sub-techniques of T*).
