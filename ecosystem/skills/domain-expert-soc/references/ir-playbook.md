# IR Playbook — Top 4 Incident Classes

Compact playbooks. Each follows NIST SP 800-61 phases. Concrete actions, named
owners, and exit criteria.

## 1. Phishing-delivered malware on endpoint

**Trigger**: EDR flags suspicious child of office product / user reports popup.

### Detect & Analyse (0-30 min)
- L1 reviews EDR detail: parent process, command line, network connections.
- Confirm malicious by: VT lookup of hash, known-bad domain, sandbox detonation.
- Collect IOCs: file hash, parent email msg-id (mail gw search), C2 IPs/domains.

### Contain (30-90 min)
- EDR network-isolate the host (preserves remote IR access).
- Disable user account in IdP.
- Block C2 IPs/domains at egress firewall + DNS sinkhole.
- Mail gateway: search-and-purge same campaign hits across all mailboxes.

### Investigate (parallel, 1-4 h)
- Did the user click a link or open an attachment? (Determines T1566 sub.)
- Did the malware achieve persistence? (Run keys, scheduled tasks, services.)
- Did it move laterally? (Other hosts contacting same C2.)
- Were credentials dumped? (LSASS process accessed?)

### Eradicate (2-8 h)
- Rebuild the host from gold image. Do **not** clean in place.
- Force password reset for the user, all their tokens revoked.
- If lateral movement: extend to affected hosts.

### Recover (4-24 h)
- Re-image complete, user gets new endpoint.
- Monitor closely for 30 days.
- User completes phishing refresher training.

### Lessons (within 2 weeks)
- Review the email — was it blocked by mail gw? If no, tune.
- Review the EDR detection — was it a high-confidence alert? Tune if noisy.
- Was IOC sharing extracted into TIP/MISP? File the report.

---

## 2. Ransomware (active encryption)

**Trigger**: mass file rename alert / user reports "files renamed to .lock" / EDR fires ransomware-behaviour rule.

### Detect & Analyse (0-15 min — speed critical)
- Confirm: at least 3 hosts encrypting OR ≥ 1,000 files encrypted on 1 host.
- Sev-1 declaration. Page on-call IR commander + leadership + legal + comms.
- Capture: ransom note content, file extension, encryption rate, lateral movement timeline.

### Contain (15-60 min)
- **Coordinated cut-over** (single moment):
  - Disable affected switches at the patch panel OR pull cables OR EDR mass-isolate.
  - Disable AD accounts for any user that touched affected hosts in last 24 h.
  - Take backup repository offline (read-only or air-gap).
  - Block egress to known ransomware C2 / leak-site IPs.
- DO NOT power off encrypted hosts yet — memory holds encryption keys.
- DO NOT reboot — same reason.

### Investigate (parallel, 1-12 h)
- How did they get in? (Often Citrix/RDP/VPN CVE, phish, or supply chain.)
- How long was dwell time? (Confirm via earliest evidence.)
- Was data exfiltrated **before** encryption? Most ransomware groups (Lockbit, BlackCat, Akira) double-extort. Check egress flows for the dwell period.
- Were domain admin creds stolen?
- Are immutable backups intact?

### Eradicate (12-72 h)
- Rebuild every encrypted host from gold image.
- Rotate **every** credential that may have been exposed: domain admin, all users on affected hosts, all service accounts, all API keys.
- Rotate KRBTGT account twice (golden ticket defense).
- Patch the entry vector.

### Recover (1-30 d)
- Restore from clean immutable backup.
- Stage restoration: critical services first, then user data.
- Monitor closely; ransomware groups often retain footholds.
- Define exit criteria: 30 days clean, no IOC hits.

### Pay-or-not decision (legal + leadership)
- Generally do not pay. Reasons: 30 % of payers don't get working decryptor, payment funds further ops, may violate OFAC sanctions if actor is on SDN list.
- If business-critical and backups are gone: engage specialist negotiator (Coveware, Kivu), legal counsel, cyber-insurance.
- Notify law enforcement (FBI IC3 in US, NCSC in UK).
- Notify regulators per jurisdiction within statutory window (e.g. GDPR 72 h, HIPAA 60 d).

### Lessons
- Backup architecture audit — were backups immutable, were they restorable?
- Detection of pre-ransom staging (recon, lateral, dumping creds).
- MFA coverage, especially on edge (VPN, RDP gateway).

---

## 3. Cloud account takeover (AWS / Azure / GCP)

**Trigger**: anomalous API activity, impossible-travel sign-in to console, MFA fatigue prompt accepted, billing alert for crypto-mining resources.

### Detect & Analyse (0-30 min)
- Identify compromised principal: IAM user, federated user, service account.
- Pull recent API call log: CloudTrail / Azure Activity Log / GCP Cloud Audit Logs.
- Classify the activity:
  - Resource creation (likely crypto-mining).
  - Data access / S3 enumeration (data theft).
  - IAM modification (persistence / priv-esc).
  - Other.

### Contain (30-90 min)
- Disable the principal (delete access keys, revoke session tokens, force MFA re-enrolment).
- For federated user: also disable in source IdP.
- Block known-bad source IPs at IAM policy / WAF level.
- Snapshot or freeze any resources created by the attacker (do not delete yet — evidence).

### Investigate (1-8 h)
- Initial vector:
  - Leaked access key (GitHub commit history, exposed .env, log file)?
  - Phish-into-IdP + MFA fatigue?
  - Supply-chain compromise of a CI/CD tool?
- Persistence:
  - New IAM users / roles?
  - Assume-role policies modified?
  - SSH keys / instance metadata access?
  - Lambda functions, EC2 user-data hooks, EventBridge rules?
- Lateral:
  - Did attacker pivot to other accounts via cross-account role assumption?
  - On-prem AD federation: did they hit on-prem?

### Eradicate (8-48 h)
- Delete attacker-created resources (after evidence preservation).
- Revoke all sessions org-wide for the affected principal.
- Audit and remove any IAM persistence (extra users, attached policies, role trust modifications).
- Rotate **every** secret the compromised principal could read (Secrets Manager / Key Vault / Secret Manager content + ALL secrets stored in plaintext anywhere).

### Recover (1-7 d)
- Re-enable the principal with new credentials and stricter policy (least privilege).
- Enforce MFA on all human IAM users (no exceptions).
- Move long-lived access keys → short-lived federation (SAML, OIDC).
- Enable GuardDuty / Defender for Cloud / Security Command Center if not already.

### Lessons
- How were credentials exposed? — secret scanning, dev workstation hygiene.
- MFA enforcement coverage.
- IAM policies — number of admin-level principals (target ≤ 10 in any account).
- Logging coverage — was CloudTrail enabled in all regions, all accounts, with management + data events?

---

## 4. Insider data exfiltration

**Trigger**: DLP alert / abnormal egress volume / departing employee accessing unusual file shares / leaver-list trigger.

### Detect & Analyse (0-2 h)
- Confirm activity: review DLP alert detail, file paths, destination, time pattern.
- Pull user activity 30-90 days back: SaaS app logs, file-share access, email rules.
- Confirm context: is this user a current employee, contractor, on PIP, departing?

### Triage call: criminal-vs-civil
- HR + legal involved before user is approached.
- Do not tip off the user. Continue silent monitoring; this is one of the few cases where you delay containment.

### Contain (timing: when legal says go)
- Single coordinated step:
  - Disable accounts (corp IdP, SaaS, VPN, MFA).
  - Revoke device tokens.
  - Recall corp laptop via MDM (lock + wipe).
  - If on-prem: badge access revoked.
- HR-led conversation with employee.

### Investigate (1-7 d)
- Full timeline of data accessed and exfiltrated.
- Methods: email-to-personal, personal cloud sync, USB, AirDrop, encrypted archive upload, screen-capture-and-photograph (very hard to catch).
- Scope: which data classifications, regulatory implications.

### Eradicate
- Less "eradicate", more "remediate":
  - Contractually enforce data return / deletion.
  - Legal action if warranted.
  - Reset shared service accounts / API keys the user had access to.

### Lessons
- DLP rule effectiveness — did our rules catch the exfil, or did we find it from a side channel?
- Leaver process — was access removal timely (target: < 4 h from termination)?
- Need-to-know enforcement — should the user have had access to this data class at all?
- Egress monitoring for personal-cloud destinations.

---

## Cross-incident standards

### Comms
- IR commander runs the bridge. ONE designated commander per incident.
- Updates every 30 min in war room, every 60 min to leadership during Sev-1, every 4 h for Sev-2.
- External comms (customers, regulators, press) ONLY through approved comms lead. Engineers do not talk to journalists.

### Documentation
- Timeline doc, updated live.
- Every action: who, when (UTC), what, why, expected outcome.
- Evidence: preserved per chain-of-custody from the moment severity is declared.

### Tooling discipline
- Use out-of-band comms (Signal, separate Slack workspace, phone bridge) for IR coordination — assume the primary corp channel is compromised.
- Document everything in a shared doc the IR team can write to, with version history.
