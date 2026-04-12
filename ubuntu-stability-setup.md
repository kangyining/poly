# Ubuntu 24/7 Stability Changes

Applied to this machine to keep the trading bot running reliably. Each section shows what was changed and the exact revert command.

Items that were reviewed but **not applied** are noted with the reason skipped.

---

## 1. Disable Sleep / Suspend / Hibernate

**What:** Prevents the OS from ever sleeping, suspending, or hibernating.

**Applied:**
```bash
sudo systemctl mask sleep.target suspend.target hibernate.target hybrid-sleep.target
```

**Revert:**
```bash
sudo systemctl unmask sleep.target suspend.target hibernate.target hybrid-sleep.target
```

---

## 2. Kernel Panic Auto-Reboot

**What:** Reboots the machine 10 seconds after a kernel panic instead of hanging forever.

**File:** `/etc/sysctl.conf`

**Applied (appended):**
```
kernel.panic=10
```

Note: `kernel.panic_on_oops=1` was reviewed and skipped — it promotes minor kernel warnings
into full panics, which causes unnecessary reboots on a stable system.

**Revert:** Remove the `kernel.panic=10` line from `/etc/sysctl.conf`, then:
```bash
sudo sysctl -p
```

---

## 3. Disable Automatic Updates

**What:** Stops `apt-daily.timer` (package list downloads) and `apt-daily-upgrade.timer` (automatic installs) from running in the background. Prevents disk I/O spikes, CPU spikes from dpkg post-install scripts, and apt lock contention during bot execution. Important now, critical when moving to HFT.

**You are responsible for running updates manually:**
```bash
sudo apt update && sudo apt upgrade
```

**Applied:**
```bash
sudo systemctl disable --now apt-daily.timer apt-daily-upgrade.timer
```

**Revert:**
```bash
sudo systemctl enable --now apt-daily.timer apt-daily-upgrade.timer
```

---

## Reviewed but not applied

| Item | Reason skipped |
|------|---------------|
| Lid close / logind.conf | Desktop — no lid |
| Screen blanking | Cosmetic only — blank screen doesn't affect background processes |
| Unattended auto-reboot | Already off by default (commented default is `false`) |
| Add swap | Already have 8 GB swap at `/swap.img` |
| Reduce swappiness | Only matters under memory pressure; bot is a lightweight Python process |
| NetworkManager auto-reconnect | Already default NM behaviour; the setting only affects boot-time retries, not runtime drops |
| DNS change | No evidence of DNS issues; ISP DNS is fine unless problems arise |
| `kernel.panic_on_oops=1` | Too aggressive — turns minor kernel warnings into reboots |
| systemd watchdog | Guards against systemd hanging, which is extremely rare on stable Ubuntu |

---

## Full Revert

```bash
# 1. Unmask sleep
sudo systemctl unmask sleep.target suspend.target hibernate.target hybrid-sleep.target

# 2. kernel.panic — remove kernel.panic=10 from /etc/sysctl.conf
sudo nano /etc/sysctl.conf
sudo sysctl -p

# 3. Re-enable automatic updates
sudo systemctl enable --now apt-daily.timer apt-daily-upgrade.timer
```
